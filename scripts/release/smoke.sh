#!/bin/sh
# Smoke test of a release artifact, used the way a user would use it.
# Usage: smoke.sh <artifact> <repo-root>
#   artifact: .zip or .tar.gz (unpacked), -setup.exe (installed silently for the current user,
#             then uninstalled), .deb (installed with apt, then removed) or .dmg (mounted).
# Environment: EXPECT_SIGNED=true makes the check fail when the artifact is not properly signed
#   (Windows: a valid Authenticode signature on the installer and the installed programs; macOS: a
#   Developer ID signature accepted by Gatekeeper and a stapled notarisation ticket). Anything else
#   only prints what the signature looks like.
# Checks: --version/--help of the programs, atomik-render of an example project to a WAV that
# is not silent, atomik-play --help; on macOS the .app is verified; on Linux the shared
# libraries are printed. No window is opened and no plugin is scanned.
set -eu
archive="$1"
root="$2"
expect_signed="${EXPECT_SIGNED:-false}"
work="$(mktemp -d)"
cleanup_cmd=""
cleanup() {
  if [ -n "$cleanup_cmd" ]; then
    eval "$cleanup_cmd" || true
  fi
}
trap cleanup EXIT

# Version, help and a render of the demo project, with the programs in <bindir>.
check_programs() {
  bindir="$1"
  ext="$2"
  for b in atomik atomik-render atomik-play atomik-plugin-scan atomik-plugin-host; do
    test -f "$bindir/$b$ext" || test -L "$bindir/$b$ext" || { echo "error: $b$ext missing in $bindir" >&2; exit 1; }
  done

  echo "== version and help"
  "$bindir/atomik$ext" --version
  "$bindir/atomik$ext" --help > /dev/null
  "$bindir/atomik-render$ext" --version
  "$bindir/atomik-play$ext" --version
  "$bindir/atomik-play$ext" --help > /dev/null

  echo "== render"
  wav="$work/smoke.wav"
  "$bindir/atomik-render$ext" "$root/examples/projects/demo.atomik" -o "$wav"
  test -s "$wav" || { echo "error: no WAV written" >&2; exit 1; }
  python3 -I - "$wav" <<'PY'
import struct, sys
data = open(sys.argv[1], "rb").read()
assert data[:4] == b"RIFF" and data[8:12] == b"WAVE", "not a WAV file"
pos, fmt, pcm = 12, None, None
while pos + 8 <= len(data):
    cid, size = data[pos:pos+4], struct.unpack("<I", data[pos+4:pos+8])[0]
    body = data[pos+8:pos+8+size]
    if cid == b"fmt ":
        fmt = struct.unpack("<HHIIHH", body[:16])
    elif cid == b"data":
        pcm = body
    pos += 8 + size + (size & 1)
assert fmt and pcm, "missing fmt or data chunk"
tag, ch, rate, _, _, bits = fmt
if tag in (3, 65534) and bits == 32:
    n = len(pcm) // 4
    peak = max(abs(v) for v in struct.unpack("<%df" % n, pcm[:n*4]))
elif bits == 16:
    n = len(pcm) // 2
    peak = max(abs(v) for v in struct.unpack("<%dh" % n, pcm[:n*2])) / 32768
else:
    raise SystemExit("unexpected WAV format %r" % (fmt,))
print("wav: %d ch, %d Hz, %d samples, peak %.4f" % (ch, rate, n, peak))
assert n > 0 and peak > 0.001, "rendered WAV is silent"
PY
  rm -f "$wav"
}

# Windows: signtool verify /pa on each given file (a Unix path), failing on the first bad one.
windows_signatures() {
  args=""
  for f in "$@"; do
    args="$args $(cygpath -w "$f")"
  done
  # shellcheck disable=SC2086
  powershell.exe -NoProfile -File "$(cygpath -w "$root/scripts/release/verify-windows.ps1")" $args
}

# The five programs of a folder, for windows_signatures.
windows_programs() {
  for b in atomik atomik-render atomik-play atomik-plugin-scan atomik-plugin-host; do
    echo "$1/$b.exe"
  done
}

smoke_archive() {
  case "$archive" in
    *.zip)
      case "$(uname -s)" in
        Darwin) ditto -x -k "$archive" "$work" ;;
        *) 7z x -y "-o$work" "$archive" > /dev/null ;;
      esac
      ;;
    *) tar -xzf "$archive" -C "$work" ;;
  esac
  dir="$(find "$work" -mindepth 1 -maxdepth 1 -type d | head -n 1)"
  echo "unpacked: $dir"
  ls -l "$dir"

  ext=""
  bindir="$dir"
  case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*) ext=".exe" ;;
    Darwin) bindir="$dir/Atomik Rack.app/Contents/MacOS" ;;
  esac
  for f in LICENSE README.md CHANGELOG.md; do
    test -f "$dir/$f" || { echo "error: $f missing" >&2; exit 1; }
  done
  check_programs "$bindir" "$ext"

  case "$(uname -s)" in
    Darwin)
      echo "== app bundle"
      app="$dir/Atomik Rack.app"
      codesign --verify --deep --strict --verbose=2 "$app"
      plutil -p "$app/Contents/Info.plist"
      lipo -archs "$bindir/atomik"
      otool -L "$bindir/atomik"
      ;;
    Linux)
      echo "== shared libraries (atomik)"
      ldd "$bindir/atomik"
      if ldd "$bindir/atomik" | grep -q "not found"; then
        echo "error: missing libraries" >&2
        exit 1
      fi
      ;;
    MINGW*|MSYS*|CYGWIN*)
      if [ "$expect_signed" = "true" ]; then
        echo "== signatures"
        # shellcheck disable=SC2046
        windows_signatures $(windows_programs "$bindir")
      fi
      ;;
  esac
}

smoke_deb() {
  echo "== package"
  dpkg-deb --info "$archive"
  dpkg-deb --contents "$archive"
  # apt resolves the Depends of the package; it needs a path with a slash.
  case "$archive" in /*) deb="$archive" ;; *) deb="./$archive" ;; esac
  sudo apt-get update -q
  sudo apt-get install -y -q "$deb"
  cleanup_cmd="sudo apt-get remove -y -q atomik-rack"
  dpkg -s atomik-rack
  # The programs are symlinks into /opt/atomik-rack: the app must find its siblings from there.
  for b in atomik atomik-render atomik-play atomik-plugin-scan atomik-plugin-host; do
    real="$(readlink -f "/usr/bin/$b")"
    echo "/usr/bin/$b -> $real"
    test -x "$real" || { echo "error: /usr/bin/$b does not resolve to a program" >&2; exit 1; }
    test "$(dirname "$real")" = "/opt/atomik-rack" || { echo "error: $b is not in /opt/atomik-rack" >&2; exit 1; }
  done
  check_programs /usr/bin ""
  echo "== shared libraries (atomik)"
  ldd /usr/bin/atomik
  if ldd /usr/bin/atomik | grep -q "not found"; then
    echo "error: missing libraries" >&2
    exit 1
  fi
  test -f /usr/share/applications/atomik-rack.desktop || { echo "error: no desktop entry" >&2; exit 1; }
  echo "== remove"
  sudo apt-get remove -y -q atomik-rack
  cleanup_cmd=""
  if [ -e /usr/bin/atomik ] || [ -e /opt/atomik-rack ]; then
    echo "error: files are left after removing the package" >&2
    exit 1
  fi
}

smoke_setup() {
  # A silent per-user install into a temporary folder, so the runner needs no elevation.
  # Git Bash would rewrite the /SWITCHES as paths: switch that off for the installer.
  appdir="$(cygpath -w "$work/app")"
  log="$(cygpath -w "$work/setup.log")"
  echo "== install (silent, current user)"
  if ! MSYS2_ARG_CONV_EXCL='*' "$archive" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /CURRENTUSER \
    "/DIR=$appdir" "/LOG=$log" /TASKS=associate; then
    cat "$work/setup.log" || true
    echo "error: the installer failed" >&2
    exit 1
  fi
  for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    test -f "$work/app/atomik.exe" && break
    sleep 3
  done
  ls -l "$work/app"
  test -f "$work/app/unins000.exe" || { echo "error: no uninstaller was installed" >&2; exit 1; }
  for f in LICENSE README.md CHANGELOG.md; do
    test -f "$work/app/$f" || { echo "error: $f missing" >&2; exit 1; }
  done
  start_menu="$(cygpath -u "$APPDATA")/Microsoft/Windows/Start Menu/Programs/Atomik Rack.lnk"
  test -f "$start_menu" || { echo "error: no Start menu shortcut at $start_menu" >&2; exit 1; }
  echo "Start menu shortcut: ok"
  # The file association is per user here (HKCU), written by the "associate" task.
  MSYS2_ARG_CONV_EXCL='*' reg query 'HKCU\Software\Classes\.atomik' /ve
  MSYS2_ARG_CONV_EXCL='*' reg query 'HKCU\Software\Classes\AtomikRack.Project\shell\open\command' /ve

  check_programs "$work/app" ".exe"

  if [ "$expect_signed" = "true" ]; then
    echo "== signatures"
    # shellcheck disable=SC2046
    windows_signatures "$archive" $(windows_programs "$work/app")
  else
    powershell.exe -NoProfile -Command "Get-AuthenticodeSignature -LiteralPath '$(cygpath -w "$archive")' | Format-List Status, StatusMessage" || true
  fi

  echo "== uninstall"
  MSYS2_ARG_CONV_EXCL='*' "$work/app/unins000.exe" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART
  # The uninstaller hands over to a copy of itself and returns at once: wait for it.
  for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    test -f "$work/app/atomik.exe" || break
    sleep 3
  done
  if [ -f "$work/app/atomik.exe" ]; then
    echo "error: the uninstaller left atomik.exe behind" >&2
    exit 1
  fi
  if [ -f "$start_menu" ]; then
    echo "error: the uninstaller left the Start menu shortcut behind" >&2
    exit 1
  fi
  echo "uninstalled"
}

smoke_dmg() {
  mnt="$work/mnt"
  mkdir -p "$mnt"
  hdiutil attach -nobrowse -readonly -mountpoint "$mnt" "$archive"
  cleanup_cmd="hdiutil detach -force \"$mnt\" > /dev/null"
  ls -l "$mnt"
  app="$mnt/Atomik Rack.app"
  test -d "$app" || { echo "error: no Atomik Rack.app in the disk image" >&2; exit 1; }
  test -L "$mnt/Applications" || { echo "error: no Applications link in the disk image" >&2; exit 1; }
  check_programs "$app/Contents/MacOS" ""
  echo "== app bundle"
  codesign --verify --deep --strict --verbose=2 "$app"
  plutil -p "$app/Contents/Info.plist"
  lipo -archs "$app/Contents/MacOS/atomik"
  echo "== signature"
  codesign --display --verbose=2 "$app" 2>&1 | grep -E '^(Authority|TeamIdentifier|Signature|flags)' || true
  if [ "$expect_signed" = "true" ]; then
    codesign --verify --strict --verbose=2 "$archive"
    spctl --assess --type execute --verbose=2 "$app"
    spctl --assess --type open --context context:primary-signature --verbose=2 "$archive"
    xcrun stapler validate "$app"
    xcrun stapler validate "$archive"
  else
    echo "(not notarised: Gatekeeper would warn on first launch)"
  fi
}

case "$archive" in
  *.zip|*.tar.gz) smoke_archive ;;
  *-setup.exe) smoke_setup ;;
  *.deb) smoke_deb ;;
  *.dmg) smoke_dmg ;;
  *) echo "error: unknown artifact type: $archive" >&2; exit 1 ;;
esac
echo "smoke ok: $(basename "$archive")"
