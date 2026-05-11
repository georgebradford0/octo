#!/usr/bin/env bash
set -euo pipefail

# Tear down every container started by start_dev.sh — lair plus all managed
# child agents — and reap their named volumes. The dev-data/ bind mount on
# the host is preserved so a follow-up start_dev.sh keeps the Noise keypair.

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

echo ""
echo "✓ Dev environment stopped."
echo "  (dev-data/ on the host preserved; remove it manually for a clean restart.)"
