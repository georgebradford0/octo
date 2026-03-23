#!/usr/bin/env sh
set -e

# ── Required ──────────────────────────────────────────────────────────────────
if [ -z "$GIT_URL" ]; then
    echo "ERROR: GIT_URL is required" >&2
    exit 1
fi
if [ -z "$ANTHROPIC_API_KEY" ]; then
    echo "ERROR: ANTHROPIC_API_KEY is required" >&2
    exit 1
fi

# ── Git authentication ────────────────────────────────────────────────────────
# If a token is provided, embed it in the URL for HTTPS auth
if [ -n "$GIT_TOKEN" ]; then
    # Strip any existing credentials from the URL, then inject the token
    CLONE_URL=$(echo "$GIT_URL" | sed 's|https://\(.*@\)\?|https://'"$GIT_TOKEN"'@|')
else
    CLONE_URL="$GIT_URL"
    # SSH key: expect /root/.ssh/id_rsa mounted by the user
    if [ -f /root/.ssh/id_rsa ]; then
        chmod 600 /root/.ssh/id_rsa
        ssh-keyscan github.com gitlab.com bitbucket.org >> /root/.ssh/known_hosts 2>/dev/null
    fi
fi

# ── Clone or update repo ──────────────────────────────────────────────────────
WORKSPACE=/workspace
if [ -d "$WORKSPACE/.git" ]; then
    echo "[claudulhu] Updating existing repo at $WORKSPACE"
    git -C "$WORKSPACE" remote set-url origin "$CLONE_URL"
    git -C "$WORKSPACE" fetch --all
else
    echo "[claudulhu] Cloning $GIT_URL into $WORKSPACE"
    git clone "$CLONE_URL" "$WORKSPACE"
fi

# ── Git identity (needed for commits / pushes) ────────────────────────────────
GIT_USER_NAME="${GIT_USER_NAME:-claudulhu}"
GIT_USER_EMAIL="${GIT_USER_EMAIL:-claudulhu@localhost}"
git -C "$WORKSPACE" config user.name  "$GIT_USER_NAME"
git -C "$WORKSPACE" config user.email "$GIT_USER_EMAIL"

# Keep credentials cached for push operations
if [ -n "$GIT_TOKEN" ]; then
    git -C "$WORKSPACE" config credential.helper \
        "!f() { echo username=x-token; echo password=$GIT_TOKEN; }; f"
fi

# ── Write claudulhu config ────────────────────────────────────────────────────
mkdir -p /root/.claudulhu
printf '{"repo":"%s"}\n' "$WORKSPACE" > /root/.claudulhu/config.json

echo "[claudulhu] Starting server (repo: $WORKSPACE)"
exec claudulhu-server
