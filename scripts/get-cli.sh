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
COMPLETIONS_INSTALLED=""
case "$DETECTED_SHELL" in
  zsh)
    COMP_DIR="$HOME/.zfunc"
    COMP_FILE="$COMP_DIR/_octo"
    mkdir -p "$COMP_DIR"
    "$INSTALL_DIR/$BIN" completions zsh > "$COMP_FILE"
    echo "Zsh completions installed to $COMP_FILE"
    ZSHRC="$HOME/.zshrc"
    if ! grep -q 'fpath.*\.zfunc' "$ZSHRC" 2>/dev/null; then
      printf '\nfpath+=~/.zfunc\nautoload -Uz compinit && compinit\n' >> "$ZSHRC"
      echo "Added fpath and compinit to $ZSHRC"
    fi
    COMPLETIONS_INSTALLED=1
    ;;
  bash)
    COMP_FILE="$HOME/.local/share/bash-completion/completions/octo"
    mkdir -p "$(dirname "$COMP_FILE")"
    "$INSTALL_DIR/$BIN" completions bash > "$COMP_FILE"
    echo "Bash completions installed to $COMP_FILE"
    # Source the file directly from ~/.bashrc so it works even without the
    # bash-completion package (which is required for the XDG directory to be
    # picked up automatically). Match the exact source line so unrelated
    # mentions of "octo" elsewhere in .bashrc don't suppress the append.
    BASHRC="$HOME/.bashrc"
    SOURCE_LINE=". $COMP_FILE"
    if ! grep -qxF "$SOURCE_LINE" "$BASHRC" 2>/dev/null; then
      printf '\n%s\n' "$SOURCE_LINE" >> "$BASHRC"
      echo "Added completion source to $BASHRC"
    fi
    COMPLETIONS_INSTALLED=1
    ;;
  fish)
    COMP_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/fish/completions"
    mkdir -p "$COMP_DIR"
    "$INSTALL_DIR/$BIN" completions fish > "$COMP_DIR/octo.fish"
    echo "Fish completions installed to $COMP_DIR/octo.fish"
    COMPLETIONS_INSTALLED=1
    ;;
  *)
    echo "Completions: run 'octo completions <bash|zsh|fish>' to generate for your shell."
    ;;
esac

echo ""
if [ -n "$COMPLETIONS_INSTALLED" ]; then
  echo "Tab-completions are installed but won't be active in this shell session."
  echo "Start a new shell (or run 'exec $DETECTED_SHELL') to activate them."
  echo ""
fi
echo "Next: run 'octo init --anthropic-api-key <key>' to bootstrap a lair Docker"
echo "      container on this host. Optional flags: --gh-token, --openai-api-key,"
echo "      --api-url, --model. All values persist to ~/.octo/config.json."
echo ""

"$INSTALL_DIR/$BIN" --help
