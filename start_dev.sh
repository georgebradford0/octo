#!/usr/bin/env bash
set -euo pipefail

# Local dev loop: build lair:dev from the working tree, launch it against a
# dedicated host data dir (`./dev-data/`), and bind-mount the Docker socket so
# the dev lair can spawn agent containers exactly like a prod install.
#
# Stop with ./stop_dev.sh — that script teardown also rms every managed agent
# container created during the session.

DEV_IMAGE="lair:dev"
DEV_DATA_DIR="$(pwd)/dev-data"
DEV_ENV_FILE="$(pwd)/dev-data.env"
DEV_NOISE_PORT="${DEV_NOISE_PORT:-9000}"
DEV_HTTP_PORT="${DEV_HTTP_PORT:-8000}"

# ── Checks ─────────────────────────────────────────────────────────────────────
if [ -z "${ANTHROPIC_API_KEY_OCTO:-}" ]; then
    echo "ERROR: ANTHROPIC_API_KEY_OCTO is not set" >&2
    exit 1
fi

if ! docker info >/dev/null 2>&1; then
    echo "ERROR: docker daemon not reachable (is Docker Desktop / dockerd running?)" >&2
    exit 1
fi

# ── Build image locally ────────────────────────────────────────────────────────
echo "▸ Building local image ${DEV_IMAGE}..."
docker build -f lair/Dockerfile -t "${DEV_IMAGE}" .

# ── Dev data dir ───────────────────────────────────────────────────────────────
mkdir -p "${DEV_DATA_DIR}"

# ── Env file consumed by `docker run --env-file` ──────────────────────────────
cat > "${DEV_ENV_FILE}" <<EOF
ANTHROPIC_API_KEY=${ANTHROPIC_API_KEY_OCTO}
GH_TOKEN=${GH_TOKEN:-}
PUBLIC_PORT=${DEV_NOISE_PORT}
NOISE_PORT=9000
OCTO_DATA_DIR=/data
NOISE_KEY_FILE=/data/noise_key.bin
OCTO_DEV=1
OCTO_SKIP_SHELL_ENV=1
EOF
chmod 600 "${DEV_ENV_FILE}"

# ── Run lair ───────────────────────────────────────────────────────────────────
echo "▸ Removing any existing lair container..."
docker rm -f lair >/dev/null 2>&1 || true

echo "▸ Running ${DEV_IMAGE}..."
docker run -d \
    --name lair \
    --label octo.managed=1 \
    --label octo.role=lair \
    -p "${DEV_NOISE_PORT}:9000" \
    -p "127.0.0.1:${DEV_HTTP_PORT}:8000" \
    -v /var/run/docker.sock:/var/run/docker.sock \
    -v "${DEV_DATA_DIR}:/data" \
    --env-file "${DEV_ENV_FILE}" \
    --restart unless-stopped \
    "${DEV_IMAGE}" >/dev/null

echo "▸ Waiting for lair to be ready on http://127.0.0.1:${DEV_HTTP_PORT}..."
for i in $(seq 1 60); do
    if curl -sf "http://127.0.0.1:${DEV_HTTP_PORT}/health" >/dev/null; then
        echo ""
        echo "✓ lair is up. Noise listener on 0.0.0.0:${DEV_NOISE_PORT}; data dir ${DEV_DATA_DIR}."
        echo "  Tail logs:   docker logs -f lair"
        echo "  Tear down:   ./stop_dev.sh"
        exit 0
    fi
    sleep 1
done

echo "ERROR: lair did not become healthy in 60s. Last logs:" >&2
docker logs --tail 100 lair >&2 || true
exit 1
