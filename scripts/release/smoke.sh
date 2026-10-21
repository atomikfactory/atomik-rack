#!/bin/sh
# Smoke test of a packaged archive, unpacked the way a user would.
# Usage: smoke.sh <archive> <repo-root>   (archive: .zip or .tar.gz)
# Checks: --version/--help of the programs, atomik-render of an example project to a WAV that
# is not silent, atomik-play --help; on macOS the .app is run and verified; on Linux the shared
# libraries are printed. No window is opened.
set -eu
archive="$1"
root="$2"
work="$(mktemp -d)"
case "$archive" in
  *.zip)
    case "$(uname -s)" in
      Darwin) ditto -x -k "$archive" "$work" ;;
      *) 7z x -y "-o$work" "$archive" > /dev/null ;;
    esac
    ;;
  *.tar.gz) tar -xzf "$archive" -C "$work" ;;
  *) echo "error: unknown archive type: $archive" >&2; exit 1 ;;
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
for b in atomik atomik-render atomik-play atomik-plugin-scan; do
  test -f "$bindir/$b$ext" || { echo "error: $b$ext missing in $bindir" >&2; exit 1; }
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
esac
echo "smoke ok: $(basename "$archive")"
