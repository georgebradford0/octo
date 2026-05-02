#!/usr/bin/env sh
set -e

REPO="georgebradford0/claudulhu"
BIN="claudulhu"
INSTALL_DIR="$HOME/.local/bin"

OS=$(uname -s)
ARCH=$(uname -m)

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64)  ARTIFACT="claudulhu-linux-x86_64" ;;
      aarch64) ARTIFACT="claudulhu-linux-aarch64" ;;
      *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      x86_64)  ARTIFACT="claudulhu-macos-x86_64" ;;
      arm64)   ARTIFACT="claudulhu-macos-aarch64" ;;
      *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
    esac
    ;;
  *) echo "Unsupported OS: $OS"; exit 1 ;;
esac

URL="https://github.com/${REPO}/releases/latest/download/${ARTIFACT}"

mkdir -p "$INSTALL_DIR"

echo "Downloading $ARTIFACT..."
curl -fsSL "$URL" -o "$INSTALL_DIR/$BIN"
chmod +x "$INSTALL_DIR/$BIN"

echo "Installed to $INSTALL_DIR/$BIN"

# Warn if ~/.local/bin is not in PATH.
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "Add to your shell: export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
esac

"$INSTALL_DIR/$BIN" --help
