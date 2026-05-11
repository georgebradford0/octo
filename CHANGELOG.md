# Changelog

## [Unreleased]

### Changed

- **BREAKING — Kubernetes removed.** lair now runs as a single Docker container on a host machine and orchestrates children via the local Docker daemon. The `octo-k8s-ops` crate, `k8s/` manifests, and `docs/kubernetes-migration.md` are deleted. `octo init` no longer installs k3s; it `docker run`s lair instead, bind-mounting `~/.octo/lair/` to `/data` and the host Docker socket. Existing k8s installs need a clean `octo init` against a fresh Docker host; the prior PVC contents (`/data/noise_key.bin`, `messages.json`, `mcp.json`, `tasks.json`) can be `kubectl cp`-ed off the old lair pod and dropped into `~/.octo/lair/` on the new host before `octo init`.
- **New `~/.octo/lair/agents.json` registry** owned by lair, replacing the prior Deployment-label model. CLI reads it for `octo agents list`; mutations (`start` / `stop` / `delete`) go through Docker and are reconciled by lair's 10 s poller.
- **Removed `message_lair` / `message_child` tools** and the `LAIR_URL` plumbing that supported them. Lair is now a pure orchestrator: it creates / inspects / terminates children; user-to-child conversations happen on the child's own mobile chat. Drops a chunk of dead code (lair's HTTP client, agent's reqwest pipeline) and one env var from the agent contract.

### Added

- **Remote-agent provisioning** via three new lair tools (`mint_bootstrap_userdata`, `register_remote_agent`, `forget_agent`). The LLM composes them with any cloud-provisioning MCP (AWS, Hetzner, GCP, …) so adding a new provider requires zero lair changes. Lair's pre-generated SSH key (from `octo init`) is the authentication channel.
  - **The userdata carries no credentials.** It only trusts lair's SSH pubkey, installs Docker + git, and starts the agent in a minimal mode. API keys, the `git_url` clone, and the post-boot restart all run over the SSH connection lair opens during `register_remote_agent` — so the operator's `ANTHROPIC_API_KEY`, `GH_TOKEN`, and the repo URL never flow through the cloud provider or the provisioning MCP.
  - `register_remote_agent` orchestrates the full bootstrap: waits for the agent's `agent-info.json`, drops `config.json` with API keys, runs `git clone` (with token-rewrite + `credential.helper`) if `git_url` was given, then `docker restart`s the agent so it picks up the workspace and credentials cleanly.
  - `bootstrap::ensure_workspace` learned to detect a pre-existing `.git` dir when no `GIT_URL` is set — supports the lair-driven clone path so the agent ends up with the repo-bound system prompt.
  - **Retry + resume**: each one-shot SSH op (`ssh::write_file`, `ssh::run_script`) retries internally up to 4 times with exponential backoff (2s → 16s), absorbing sshd-during-cloud-init flakes silently. `register_remote_agent` writes a `Pending` registry row as soon as the agent's identity is known, surfacing the in-progress agent to mobile and `list_agents` immediately; if a later SSH phase hard-fails, the row stays Pending so a second `register_remote_agent` call with the same `name + host` resumes from the top (every SSH phase is idempotent). A new `Registry::set(record)` upsert replaces the prior name-conflict error for this resumable path.
  - `AgentRecord` gains `provider: Option<String>` and `metadata: serde_json::Value` for provider-side bookkeeping. The lair Docker image now ships `openssh-client`.
- **`octo init` now generates an Ed25519 SSH keypair** at `~/.octo/lair/ssh_id_ed25519{,.pub}`. Reserved for ops backchannels (e.g. tailing logs on a remote-provisioned VM); idempotent — existing keys are left untouched.
- **`/child-version` endpoint removed.** Image versions are recorded in the registry at agent-create time; no child round-trip needed.

## [0.1.2] - 2026-04-06

### Fixed
- **Session bubble preserved on reconnect** — server now assigns a UUID to each agentic session via `session_start`; mobile client uses it to find and reuse the existing session bubble when the server replays `session_start` after reconnect, preventing a duplicate stale bubble appearing alongside the live one

## [0.1.1] - 2026-04-06

### Changed
- **Session lifecycle enforced for all tool calls** — system prompt and tool descriptions now require `session_start` as the very first tool call and `session_end` as the very last, regardless of how many tools are used; previously "non-trivial work" wording left a loophole for single/quick calls

### Fixed
- **MCP child process detach** — replaced non-existent `child.forget()` with `std::mem::forget(child)` to correctly detach spawned MCP server processes from the tokio runtime

## [0.1.0] - 2026-04-06

### Added
- **Connection status dot in chat header** — 8×8 colored circle to the left of the "octo" title indicates server connection state (green = ready, yellow = connecting/streaming, red = error)

### Fixed
- **Noise tunnel re-establishment on app foreground** — AppState listener in `AppInner` now calls `NoiseConnection.disconnect()` + `NoiseConnection.connect()` when the app resumes from background, fixing silent WebSocket reconnect failures caused by iOS suspending the native Noise TCP proxy

### Changed
- **Full server + mobile rewrite** — simplified the entire system end-to-end:
  - **Server (`server/src/main.rs`)**: single session, new wire protocol (`history` / `token` / `tool` / `question` / `done` / `error`), live event buffer with generation counter for safe reconnect replay, `deliver_current` flag prevents duplicate delivery when history already contains a completed response. Removed: worker sessions, session IDs in URLs, event log (.jsonl), seq tracking, `/workers` route, UUID usage, per-session HashMaps
  - **Mobile (`mobile/App.tsx`)**: rewritten from ~1,600 lines to ~680; simplified types (`Message`, `ConnStatus`, `ServerFrame`); three clear screens (connecting spinner, connection picker, chat); token accumulation streams assistant replies inline; AsyncStorage cache per connection; `sendMessageRef` pattern retained to avoid stale closures
