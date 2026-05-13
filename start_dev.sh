#!/usr/bin/env bash
set -euo pipefail

# Local dev loop: cargo-build lair from the working tree and run it directly
# against a dedicated host data dir (./dev-data/). No Docker involved — lair
# and any agents it spawns are plain OS processes on this host.
#
# Stop with ./stop_dev.sh (kills the lair pid and any agent pids it spawned,
# rms ./dev-data/).

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
DEV_DATA_DIR="${REPO_ROOT}/dev-data/lair"
DEV_AGENTS_DIR="${REPO_ROOT}/dev-data/agents"
DEV_CONFIG_SRC="${REPO_ROOT}/config.json"
DEV_NOISE_PORT="${DEV_NOISE_PORT:-9000}"

if [ ! -f "${DEV_CONFIG_SRC}" ]; then
    echo "ERROR: ${DEV_CONFIG_SRC} is missing." >&2
    echo "       Create it (gitignored) with the same schema as ~/.octo/config.json:" >&2
    echo "       { \"anthropic_api_key\": \"sk-ant-…\", \"model\": \"claude-sonnet-4-6\" }" >&2
    exit 1
fi

# Build lair once so the spawn is fast.
echo "▸ cargo build -p octo-lair --release..."
( cd "${REPO_ROOT}" && cargo build -p octo-lair --release )

# Ensure dev dirs exist and seed config.
mkdir -p "${DEV_DATA_DIR}" "${DEV_AGENTS_DIR}" "${REPO_ROOT}/dev-data"

# In native mode, config.json is read from $HOME/.octo/config.json (via
# octo_core::config_dir). For dev, point OCTO_HOME at the dev-data dir so we
# don't smear over the operator's real config.
DEV_HOME="${REPO_ROOT}/dev-data/home"
mkdir -p "${DEV_HOME}"
install -m 600 "${DEV_CONFIG_SRC}" "${DEV_HOME}/config.json"

# Use the locally-built lair binary for both roles.
export OCTO_LAIR_BINARY="${REPO_ROOT}/target/release/octo-lair"
export OCTO_DATA_DIR="${DEV_DATA_DIR}"
export OCTO_AGENTS_DIR="${DEV_AGENTS_DIR}"
export OCTO_HOME="${DEV_HOME}"
export NOISE_PORT="${DEV_NOISE_PORT}"
export PUBLIC_PORT="${DEV_NOISE_PORT}"
export OCTO_DEV=1
export OCTO_SKIP_SHELL_ENV=1
export GH_TOKEN="${GH_TOKEN:-}"

echo "▸ Starting lair (cargo run -p octo-lair --release -- --role lair)..."
echo "  data_dir: ${DEV_DATA_DIR}"
echo "  agents_dir: ${DEV_AGENTS_DIR}"
echo "  HOME:     ${DEV_HOME}"
echo ""
exec "${OCTO_LAIR_BINARY}" --role lair
