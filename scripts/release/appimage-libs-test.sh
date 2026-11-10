#!/bin/sh
# Checks the library filter of the AppImage on a fixed list (no ldd, no build needed).
# Usage: sh scripts/release/appimage-libs-test.sh
set -eu
out="$(printf '%s\n' libc.so.6 /lib/x86_64-linux-gnu/libm.so.6 libasound.so.2 libfontconfig.so.1 \
  libfreetype.so.6 libz.so.1 libgcc_s.so.1 libvulkan.so.1 libwayland-client.so.0 libX11.so.6 \
  libxcb.so.1 libxkbcommon.so.0 libGL.so.1 libEGL.so.1 libjack.so.0 libfoo.so.3 /usr/lib/libbar.so.1 \
  | sh scripts/release/appimage-libs.sh --filter)"
want="libfoo.so.3
/usr/lib/libbar.so.1"
if [ "$out" != "$want" ]; then
  echo "error: unexpected bundle list:" >&2
  printf '%s\n' "$out" >&2
  exit 1
fi
echo "appimage-libs: ok"
