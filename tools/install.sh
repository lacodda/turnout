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

# The tag comes from the /releases/latest redirect rather than the REST API:
# unauthenticated API calls are capped at 60 per hour per IP, and an installer
# that fails because someone else on the same address ran it is no installer.
# TURNOUT_VERSION pins a specific release.
TAG="${TURNOUT_VERSION:-}"
if [ -z "$TAG" ]; then
    LOCATION=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" || true)
    TAG="${LOCATION##*/}"
fi
case "$TAG" in
    v[0-9]*) ;;
    *)
        echo "Cannot resolve the latest release of $REPO - set TURNOUT_VERSION to a tag like v0.4.0" >&2
        exit 1
        ;;
esac

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

# Short alias `tn`, unless something else in PATH already answers to that name
# (ours from a previous run does not count). TURNOUT_NO_ALIAS=1 skips it.
if [ -z "${TURNOUT_NO_ALIAS:-}" ]; then
    EXISTING=$(command -v tn 2>/dev/null || true)
    if [ -z "$EXISTING" ] || [ "$EXISTING" = "$BIN_DIR/tn" ]; then
        ln -sf turnout "$BIN_DIR/tn"
        echo "Alias tn -> turnout"
    else
        echo "Note: 'tn' already resolves to $EXISTING - alias skipped."
    fi
fi

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "Note: add $BIN_DIR to your PATH." ;;
esac
