#!/usr/bin/env sh
set -e

# ── Required ──────────────────────────────────────────────────────────────────
if [ -z "$ANTHROPIC_API_KEY" ]; then
    echo "ERROR: ANTHROPIC_API_KEY is required" >&2
    exit 1
fi

if [ -z "$PUBLIC_HOST" ]; then
    if [ "${OCTO_DEV:-0}" = "1" ]; then
        PUBLIC_HOST="127.0.0.1"
        echo "[octo-rulyeh] DEV mode: using PUBLIC_HOST=127.0.0.1"
    else
        PUBLIC_HOST=$(curl -sf --max-time 5 https://api.ipify.org || wget -qO- --timeout=5 https://api.ipify.org 2>/dev/null)
        if [ -z "$PUBLIC_HOST" ]; then
            echo "ERROR: Could not auto-detect public IP. Set PUBLIC_HOST explicitly." >&2
            exit 1
        fi
        echo "[octo-rulyeh] Detected public IP: ${PUBLIC_HOST}"
    fi
fi
export PUBLIC_HOST

NOISE_PORT="${NOISE_PORT:-9000}"
OCTO_DATA_DIR="${OCTO_DATA_DIR:-/data}"
mkdir -p "$OCTO_DATA_DIR"

# ── Startup script ────────────────────────────────────────────────────────────
if [ -n "$STARTUP_SCRIPT" ]; then
    echo "[octo-rulyeh] Running STARTUP_SCRIPT..."
    printf '%s' "$STARTUP_SCRIPT" | bash
    echo "[octo-rulyeh] STARTUP_SCRIPT complete."
fi

# ── Noise key ─────────────────────────────────────────────────────────────────
NOISE_PUBKEY=$(octo-rulyeh --print-pubkey)
echo "[octo-rulyeh] Noise public key: ${NOISE_PUBKEY}"

# ── QR code ───────────────────────────────────────────────────────────────────
# Format v2: "2:<host>:<port>:<pubkey_base32>"
# PUBLIC_PORT overrides the advertised port (e.g. the NodePort seen externally).
QR_DATA="2:${PUBLIC_HOST}:${PUBLIC_PORT:-$NOISE_PORT}:${NOISE_PUBKEY}"
SENTINEL="[rulyeh] HTTP listening on"

PIPE=$(mktemp -t octo-pipe-XXXXXX)
rm -f "$PIPE"
mkfifo "$PIPE"

octo-rulyeh 2>&1 | tee "$PIPE" &
SERVER_PID=$!

QR_PRINTED=0
while IFS= read -r line; do
    if [ "$QR_PRINTED" -eq 0 ] && \
       printf '%s' "$line" | grep -qF "$SENTINEL"; then
        echo ""
        echo "[octo-rulyeh] Scan this QR code with the app to connect:"
        echo ""
        printf '%s' "${QR_DATA}" | qrencode -l L -m 4 -t UTF8 -o -
        echo ""
        QR_PRINTED=1
    fi
done < "$PIPE"

rm -f "$PIPE"
wait "$SERVER_PID"
