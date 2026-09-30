#!/usr/bin/env bash
# Installs Frankenstein Harness (fh) for the current user on Linux or macOS. No root needed.
#   curl -fsSL https://raw.githubusercontent.com/cardimvitor/frankenstein-harness/main/scripts/install.sh | bash
#   VERSION=v0.1.0 INSTALL_DIR=$HOME/bin scripts/install.sh       (SOURCE=<folder> installs from a local folder)
set -euo pipefail
REPO="${REPO:-cardimvitor/frankenstein-harness}"
VERSION="${VERSION:-latest}"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
SOURCE="${SOURCE:-}"
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target=x86_64-unknown-linux-musl ;;
  Linux-aarch64|Linux-arm64) target=aarch64-unknown-linux-gnu ;;
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  *) echo "unsupported platform: $(uname -s) $(uname -m)" >&2; exit 1 ;;
esac
command -v git >/dev/null || echo "warning: git not found; fh needs it for per-task checkpoints" >&2
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
if [ -n "$SOURCE" ]; then
  archive="$(ls "$SOURCE"/fh-*-"$target".tar.gz | sort | tail -1)"; cp "$archive" "$tmp/"; cp "$archive.sha256" "$tmp/" 2>/dev/null || true
  name="$(basename "$archive")"
else
  if [ "$VERSION" = latest ]; then api="https://api.github.com/repos/$REPO/releases/latest"; else api="https://api.github.com/repos/$REPO/releases/tags/$VERSION"; fi
  json="$(curl -fsSL "$api")"
  url="$(printf '%s' "$json" | grep -o "https://[^\"]*fh-[^\"]*-$target\.tar\.gz" | head -1)"
  [ -n "$url" ] || { echo "no $target build in that release" >&2; exit 1; }
  name="$(basename "$url")"
  curl -fsSL -o "$tmp/$name" "$url"; curl -fsSL -o "$tmp/$name.sha256" "$url.sha256" 2>/dev/null || true
fi
if [ -f "$tmp/$name.sha256" ]; then
  want="$(cut -d' ' -f1 "$tmp/$name.sha256")"
  if command -v sha256sum >/dev/null; then have="$(sha256sum "$tmp/$name" | cut -d' ' -f1)"; else have="$(shasum -a 256 "$tmp/$name" | cut -d' ' -f1)"; fi
  [ "$want" = "$have" ] || { echo "checksum mismatch: expected $want, got $have. Nothing was installed." >&2; exit 1; }
  echo "==> checksum verified"
else
  echo "warning: no .sha256 published; download not verified" >&2
fi
tar xzf "$tmp/$name" -C "$tmp"
bin="$(find "$tmp" -name fh -type f | head -1)"
[ -n "$bin" ] || { echo "archive has no fh binary" >&2; exit 1; }
mkdir -p "$INSTALL_DIR"; install -m 755 "$bin" "$INSTALL_DIR/fh.new"; mv -f "$INSTALL_DIR/fh.new" "$INSTALL_DIR/fh"
echo "==> installed $INSTALL_DIR/fh"
case ":$PATH:" in *":$INSTALL_DIR:"*) ;; *) echo "note: add $INSTALL_DIR to your PATH (for example: export PATH=\"$INSTALL_DIR:\$PATH\")" ;; esac
"$INSTALL_DIR/fh" --help | head -1
