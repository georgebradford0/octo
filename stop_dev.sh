#!/usr/bin/env bash
set -euo pipefail

# Tear down every container started by start_dev.sh — lair plus all managed
# child agents — reap their named volumes, and delete the host-side dev
# artefacts (dev-data/, dev-data.env) so the next start_dev.sh is a clean
# slate. The repo-root config.json is left alone — it's the operator's
# credentials file, not a per-session artefact.

DEV_DATA_DIR="$(pwd)/dev-data"
DEV_ENV_FILE="$(pwd)/dev-data.env"

# Stop and remove every managed agent container (label octo.managed=1).
managed=$(docker ps -aq --filter "label=octo.managed=1" 2>/dev/null || true)
if [ -n "${managed}" ]; then
    echo "▸ Removing managed containers..."
    docker rm -f ${managed} >/dev/null 2>&1 || true
fi

# Reap orphaned agent volumes (named `agent-<name>-data` and
# `agent-<name>-workspace`).
agent_vols=$(docker volume ls -q --filter 'name=agent-' 2>/dev/null || true)
if [ -n "${agent_vols}" ]; then
    echo "▸ Removing agent volumes..."
    docker volume rm ${agent_vols} >/dev/null 2>&1 || true
fi

# Remove host-side dev artefacts created by start_dev.sh.
if [ -d "${DEV_DATA_DIR}" ]; then
    echo "▸ Removing ${DEV_DATA_DIR}..."
    rm -rf "${DEV_DATA_DIR}"
fi
if [ -f "${DEV_ENV_FILE}" ]; then
    echo "▸ Removing ${DEV_ENV_FILE}..."
    rm -f "${DEV_ENV_FILE}"
fi

echo ""
echo "✓ Dev environment stopped."
