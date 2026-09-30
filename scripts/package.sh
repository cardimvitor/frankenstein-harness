#!/usr/bin/env bash
# Packages a built binary as fh-<version>-<target>.tar.gz (or .zip on Windows) plus a .sha256 file.
#   scripts/package.sh <target-triple> [out-dir]     (expects target/<triple>/release/fh[.exe], or target/release when no --target build)
set -euo pipefail
target="${1:?usage: package.sh <target-triple> [out-dir]}"
out="${2:-dist}"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
bin=fh; case "$target" in *windows*) bin=fh.exe ;; esac
src="target/$target/release/$bin"; [ -f "$src" ] || src="target/release/$bin"
[ -f "$src" ] || { echo "binary not found for $target" >&2; exit 1; }
name="fh-$version-$target"
mkdir -p "$out/$name"
cp "$src" README.md LICENSE "$out/$name/" 2>/dev/null || cp "$src" README.md "$out/$name/"
( cd "$out"
  case "$target" in
    *windows*) 7z a -bso0 "$name.zip" "$name" >/dev/null 2>&1 || zip -qr "$name.zip" "$name"; archive="$name.zip" ;;
    *) tar czf "$name.tar.gz" "$name"; archive="$name.tar.gz" ;;
  esac
  if command -v sha256sum >/dev/null; then sha256sum "$archive" > "$archive.sha256"; else shasum -a 256 "$archive" > "$archive.sha256"; fi
  rm -rf "$name"
  echo "$out/$archive" )
