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

# Install shell completions.
DETECTED_SHELL=$(basename "${SHELL:-sh}")
case "$DETECTED_SHELL" in
  zsh)
    COMP_DIR="$HOME/.zfunc"
    mkdir -p "$COMP_DIR"
    "$INSTALL_DIR/$BIN" completions zsh > "$COMP_DIR/_claudulhu"
    echo "Zsh completions installed to $COMP_DIR/_claudulhu"
    # Check if fpath line is already present; if not, advise the user.
    if ! grep -q 'fpath.*\.zfunc' "$HOME/.zshrc" 2>/dev/null; then
      echo "Add to ~/.zshrc if not already present:"
      echo "  fpath+=~/.zfunc"
      echo "  autoload -Uz compinit && compinit"
    fi
    ;;
  bash)
    COMP_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/bash-completion/completions"
    mkdir -p "$COMP_DIR"
    "$INSTALL_DIR/$BIN" completions bash > "$COMP_DIR/claudulhu"
    echo "Bash completions installed to $COMP_DIR/claudulhu"
    ;;
  fish)
    COMP_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/fish/completions"
    mkdir -p "$COMP_DIR"
    "$INSTALL_DIR/$BIN" completions fish > "$COMP_DIR/claudulhu.fish"
    echo "Fish completions installed to $COMP_DIR/claudulhu.fish"
    ;;
  *)
    echo "Completions: run 'claudulhu completions <bash|zsh|fish>' to generate for your shell."
    ;;
esac

"$INSTALL_DIR/$BIN" --help
