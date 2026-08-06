#!/bin/sh
# turnout installer for macOS and Linux:
#   curl -fsSL https://raw.githubusercontent.com/lacodda/turnout/main/tools/install.sh | sh
set -eu

REPO="lacodda/turnout"

case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) TARGET="x86_64-unknown-linux-gnu" ;;
    Darwin-arm64) TARGET="aarch64-apple-darwin" ;;
    *)
        echo "No prebuilt binary for $(uname -s)/$(uname -m); install with: cargo install turnout" >&2
        exit 1
        ;;
esac

TAG=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | grep -o '"tag_name": *"[^"]*"' | head -n 1 | cut -d '"' -f 4)
[ -n "$TAG" ] || { echo "Cannot resolve the latest release of $REPO" >&2; exit 1; }

NAME="turnout-$TAG-$TARGET"
URL="https://github.com/$REPO/releases/download/$TAG/$NAME.tar.gz"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "Downloading $URL"
curl -fsSL "$URL" | tar xz -C "$TMP"

BIN_DIR="${TURNOUT_INSTALL_DIR:-$HOME/.local/bin}"
mkdir -p "$BIN_DIR"
install -m 755 "$TMP/$NAME/turnout" "$BIN_DIR/turnout"
echo "Installed turnout $TAG to $BIN_DIR/turnout"

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "Note: add $BIN_DIR to your PATH." ;;
esac
