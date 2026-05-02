#!/usr/bin/env sh
set -e

REPO="georgebradford0/claudulhu"
BIN="claudulhu"
INSTALL_DIR="/usr/local/bin"

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

echo "Downloading $ARTIFACT..."
curl -fsSL "$URL" -o "/tmp/$BIN"
chmod +x "/tmp/$BIN"

if [ -w "$INSTALL_DIR" ]; then
  mv "/tmp/$BIN" "$INSTALL_DIR/$BIN"
else
  sudo mv "/tmp/$BIN" "$INSTALL_DIR/$BIN"
fi

echo "Installed to $INSTALL_DIR/$BIN"
claudulhu --help
