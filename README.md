# claudulhu

An agentic coding assistant that runs a Rust server, exposing a WebSocket API for mobile and other clients to interact with a git repository via Claude.

## Docker

The server is available as a multi-platform Docker image (`linux/amd64`, `linux/arm64`):

```sh
docker run -p 8000:8000 \
  -e GIT_URL=https://github.com/user/repo \
  -e GIT_TOKEN=ghp_... \
  -e ANTHROPIC_API_KEY=sk-ant-... \
  ghcr.io/georgebradford0/claudulhu-server:latest
```

### Environment variables

| Variable | Required | Description |
|---|---|---|
| `GIT_URL` | Yes | URL of the repository to clone |
| `ANTHROPIC_API_KEY` | Yes | Anthropic API key |
| `GIT_TOKEN` | No | GitHub/GitLab personal access token (required for pushing to private repos or creating PRs) |
| `GIT_USER_NAME` | No | Git commit author name (default: `claudulhu`) |
| `GIT_USER_EMAIL` | No | Git commit author email (default: `claudulhu@localhost`) |

### Git URL schemes

Two URL schemes are supported:

**HTTPS** (`https://github.com/user/repo`)
- Clone and push authenticated via `GIT_TOKEN`
- Token is embedded in the clone URL and set as a credential helper for subsequent pushes

**SSH** (`git@github.com:user/repo.git`)
- Clone and push authenticated via SSH key
- Mount your key at `/root/.ssh/id_rsa`: `-v ~/.ssh/id_rsa:/root/.ssh/id_rsa:ro`
- `GIT_TOKEN` is ignored when using SSH URLs — if both are provided, the token is silently unused

### Multiple repos

Each container is independent. Run one per repo on different host ports:

```sh
docker run -d -p 8000:8000 -e GIT_URL=https://github.com/user/repo-a ...
docker run -d -p 8001:8000 -e GIT_URL=https://github.com/user/repo-b ...
```

### PR/MR creation

The agentic loop has a `create_pull_request` tool that opens a GitHub PR or GitLab MR after pushing a branch. Requires `GIT_TOKEN` with `repo` (GitHub) or `api` (GitLab) scope. Not available when using SSH authentication.
