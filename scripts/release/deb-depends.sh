#!/bin/sh
# Prints the Depends: value for the given ELF programs, worked out from what they link:
# the NEEDED libraries of each program mapped to Debian packages, and libc6 (>= N) with N the
# highest GLIBC symbol version any of them uses. A library this script does not know is resolved
# on the build machine with `dpkg -S` (so the build runs on Debian or Ubuntu); if that fails too
# the script stops instead of shipping a package that misses a dependency.
# Usage: deb-depends.sh <elf>...
set -eu

for tool in readelf objdump; do
  command -v "$tool" > /dev/null 2>&1 || { echo "error: $tool is needed (apt-get install binutils)" >&2; exit 1; }
done

libs="$(for f in "$@"; do readelf -d "$f" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p'; done | LC_ALL=C sort -u)"

glibc="$(for f in "$@"; do objdump -T "$f" | grep -o 'GLIBC_[0-9][0-9.]*' || true; done \
  | sed 's/^GLIBC_//' | sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1)"

deps=""
add() {
  case " $deps " in
    *" $1 "*) ;;
    *) deps="$deps $1" ;;
  esac
}
# A dependency with a space-free alternative is written with "|" and no spaces here, and spaced
# out when printed.
for lib in $libs; do
  case "$lib" in
    libc.so.*|libm.so.*|libdl.so.*|libpthread.so.*|librt.so.*|libutil.so.*|ld-linux-*.so.*)
      add libc6 ;;
    libgcc_s.so.1) add libgcc-s1 ;;
    libstdc++.so.6) add libstdc++6 ;;
    libasound.so.2) add "libasound2|libasound2t64" ;;
    libfontconfig.so.1) add libfontconfig1 ;;
    libfreetype.so.6) add libfreetype6 ;;
    libz.so.1) add zlib1g ;;
    *)
      path="$(ldconfig -p 2> /dev/null | awk -v s="$lib" '$1 == s { print $NF; exit }')"
      pkg=""
      if [ -n "$path" ]; then
        pkg="$(dpkg -S "$path" 2> /dev/null | head -n 1 | cut -d: -f1 || true)"
        if [ -z "$pkg" ]; then
          pkg="$(dpkg -S "$(readlink -f "$path")" 2> /dev/null | head -n 1 | cut -d: -f1 || true)"
        fi
      fi
      if [ -z "$pkg" ]; then
        echo "error: do not know which Debian package provides $lib; add it to deb-depends.sh" >&2
        exit 1
      fi
      add "$pkg"
      ;;
  esac
done

out=""
for d in $deps; do
  if [ "$d" = "libc6" ] && [ -n "$glibc" ]; then
    d="libc6 (>= $glibc)"
  else
    d="$(printf '%s' "$d" | sed 's/|/ | /g')"
  fi
  out="${out:+$out, }$d"
done
# libc6 is always needed, even for a program that happens not to list it.
case "$out" in
  *libc6*) ;;
  *) out="libc6 (>= ${glibc:-2.31})${out:+, $out}" ;;
esac
printf '%s\n' "$out"
