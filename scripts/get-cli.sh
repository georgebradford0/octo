#!/usr/bin/env sh
set -e

REPO="georgebradford0/octo"
BIN="octo"
INSTALL_DIR="$HOME/.local/bin"

OS=$(uname -s)
ARCH=$(uname -m)

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64)  ARTIFACT="octo-linux-x86_64" ;;
      aarch64) ARTIFACT="octo-linux-aarch64" ;;
      *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      x86_64)  ARTIFACT="octo-macos-x86_64" ;;
      arm64)   ARTIFACT="octo-macos-aarch64" ;;
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

# octo orchestrates lair and child agents as Docker containers on this host.
# Surface the Docker requirement loudly so the user doesn't run `octo init`
# blind. Soft-warn — don't exit, since some users install the CLI as part of
# a wider provisioning script that brings Docker up afterwards.
if ! command -v docker > /dev/null 2>&1; then
  echo ""
  echo "Warning: 'docker' was not found on PATH."
  echo "  octo runs lair and child agents as Docker containers; you'll need"
  echo "  Docker installed before 'octo init' will work."
  echo "  See https://docs.docker.com/get-docker/"
elif ! docker info > /dev/null 2>&1; then
  echo ""
  echo "Note: Docker is installed but its daemon isn't reachable."
  echo "  Start Docker (or the daemon socket) before running 'octo init'."
fi

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
    "$INSTALL_DIR/$BIN" completions zsh > "$COMP_DIR/_octo"
    echo "Zsh completions installed to $COMP_DIR/_octo"
    ZSHRC="$HOME/.zshrc"
    if ! grep -q 'fpath.*\.zfunc' "$ZSHRC" 2>/dev/null; then
      printf '\nfpath+=~/.zfunc\nautoload -Uz compinit && compinit\n' >> "$ZSHRC"
      echo "Added fpath and compinit to $ZSHRC"
    fi
    ;;
  bash)
    COMP_FILE="$HOME/.local/share/bash-completion/completions/octo"
    mkdir -p "$(dirname "$COMP_FILE")"
    "$INSTALL_DIR/$BIN" completions bash > "$COMP_FILE"
    echo "Bash completions installed to $COMP_FILE"
    # Source the file directly from ~/.bashrc so it works even without the
    # bash-completion package (which is required for the XDG directory to be
    # picked up automatically).
    BASHRC="$HOME/.bashrc"
    if ! grep -q "octo" "$BASHRC" 2>/dev/null; then
      printf '\n. %s\n' "$COMP_FILE" >> "$BASHRC"
      echo "Added completion source to $BASHRC"
    fi
    ;;
  fish)
    COMP_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/fish/completions"
    mkdir -p "$COMP_DIR"
    "$INSTALL_DIR/$BIN" completions fish > "$COMP_DIR/octo.fish"
    echo "Fish completions installed to $COMP_DIR/octo.fish"
    ;;
  *)
    echo "Completions: run 'octo completions <bash|zsh|fish>' to generate for your shell."
    ;;
esac

echo ""
echo "Next: run 'octo init' to bootstrap a lair Docker container on this host."
echo "      (Set ANTHROPIC_API_KEY in your env, or pass --anthropic-api-key.)"
echo ""

"$INSTALL_DIR/$BIN" --help
