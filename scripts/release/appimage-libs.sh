#!/bin/sh
# Decides which shared libraries go into the AppImage.
# Usage: appimage-libs.sh --filter        reads library names (one per line, as in NEEDED entries or
#                                         as paths) on stdin, prints the ones to bundle
#        appimage-libs.sh <elf>...        prints the paths of the libraries those programs load
#                                         (from ldd) that are to be bundled
# A library is NOT bundled when every Linux system has it with a stable interface (the C library and
# its companions), when it is tied to the machine (the sound system, the fonts, the graphics driver
# stack, the display servers and their helpers: they must match the host's drivers and configuration),
# or when the program loads it by name at run time (Vulkan, Wayland, X11, JACK are dlopen'ed, so they
# never show up as needed and always come from the host). Anything else is bundled. This is the usual
# AppImage exclude list, kept short; add a pattern here when a library turns out to belong to the host.
set -eu

excluded() {
  case "$(basename "$1")" in
    ld-linux*.so* | linux-vdso.so* | libc.so* | libm.so* | libdl.so* | libpthread.so* | librt.so* | libutil.so*) return 0 ;;
    libresolv.so* | libnss_*.so* | libnsl.so* | libcrypt.so* | libanl.so*) return 0 ;;
    libgcc_s.so* | libstdc++.so*) return 0 ;;
    libz.so* | libfontconfig.so* | libfreetype.so* | libexpat.so* | libharfbuzz*.so* | libpng*.so*) return 0 ;;
    libasound.so* | libjack*.so* | libpulse*.so* | libpipewire*.so*) return 0 ;;
    libGL.so* | libGLX*.so* | libGLdispatch.so* | libEGL.so* | libGLESv*.so* | libOpenGL.so* | libvulkan.so*) return 0 ;;
    libdrm*.so* | libgbm.so* | libxcb*.so* | libX*.so* | libwayland*.so* | libxkbcommon*.so* | libdbus-1.so*) return 0 ;;
    libglib-2.0.so* | libgobject-2.0.so* | libgio-2.0.so* | libgmodule-2.0.so* | libgdk*.so* | libgtk*.so*) return 0 ;;
    libselinux.so* | libsystemd.so* | libudev.so* | libcap.so* | libgpg-error.so*) return 0 ;;
  esac
  return 1
}

if [ "${1:-}" = "--filter" ]; then
  while IFS= read -r lib; do
    [ -n "$lib" ] || continue
    excluded "$lib" || printf '%s\n' "$lib"
  done
  exit 0
fi

[ "$#" -gt 0 ] || { echo "usage: appimage-libs.sh --filter | <elf>..." >&2; exit 1; }
for f in "$@"; do
  ldd "$f" | awk '/=> \// { print $3 }'
done | LC_ALL=C sort -u | sh "$0" --filter
