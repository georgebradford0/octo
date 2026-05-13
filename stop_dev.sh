#!/usr/bin/env bash
set -euo pipefail

# Tear down the dev lair started by start_dev.sh. Kills the lair process and
# any agent processes it spawned, then nukes the dev-data dir.

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
DEV_ROOT="${REPO_ROOT}/dev-data"

# pkill is the simplest pattern for this. The lair binary is target/release/octo-lair;
# children invoke it again with `--role agent`. Killing every octo-lair launched
# from this repo's target dir catches both.
LAIR_BIN="${REPO_ROOT}/target/release/octo-lair"
if [ -x "${LAIR_BIN}" ]; then
    echo "▸ Killing any running octo-lair processes from this tree..."
    pkill -TERM -f "${LAIR_BIN}" 2>/dev/null || true
    sleep 1
    pkill -KILL -f "${LAIR_BIN}" 2>/dev/null || true
fi

if [ -d "${DEV_ROOT}" ]; then
    echo "▸ Removing ${DEV_ROOT}..."
    rm -rf "${DEV_ROOT}"
fi

echo ""
echo "✓ Dev environment stopped."
