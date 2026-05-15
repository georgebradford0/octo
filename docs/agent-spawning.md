# Agent-spawned agents

How agents create their own child agents, how ownership is tracked, and what stops the design from collapsing into a fork-bomb or a privilege-escalation path. As of `lair 0.11.0`.

This is the companion to [agent-isolation.md](agent-isolation.md). The isolation doc explains why a child can't terminate lair or its siblings; this doc explains how a child can *legitimately* spawn its own descendants and what keeps that flow scoped.

## The shape

```
operator
   │ creates
   ▼
agent A (parent: None)         ← top-level, no spawn capability
   │ spawn_agent
   ▼
agent B (parent: "A")          ← owned by A
   │ spawn_agent
   ▼
agent C (parent: "B")          ← owned by B (grandchild of A)
```

Each `spawn_agent` carves a new node off the caller. Each `terminate_agent` lops a subtree off (leaves first). Operator-spawned agents are roots and have no spawn capability themselves — only the children they create do.

## Why operator-spawned agents can't spawn

Two complementary reasons:

1. **No token in env.** Lair only mints an `OCTO_AGENT_TOKEN` for children whose creation came in through the agent-token-gated endpoint. Operator paths (CLI `octo agents create`, lair-LLM `create_agent` tool) leave the env var unset.
2. **No tool exposed.** `lair/src/agent.rs::make_extra_tools` checks `has_spawn_capability()` (both `OCTO_AGENT_TOKEN` *and* `LAIR_INTERNAL_URL` set and non-empty) before pushing `spawn_agent` / `terminate_agent` into the tool list. If the env vars aren't there, the model literally can't see the tools.

This is by design: operators get the full surface (any name, any port, any startup script — already trusted). Agents get a constrained surface scoped to their own subtree.

## The capability token

When lair spawns an agent whose `parent` is `Some(_)`, it mints a fresh 32-byte random token, base64-url encodes it, and stores it in `AgentTokens` (an in-memory `HashMap<name, TokenEntry>`) plus persists the whole map at `/data/lair/agent-tokens.json` with `0600` perms.

```rust
// lair/src/lair.rs::exec_create_agent_for_parent
let agent_token = if parent.is_some() {
    Some(state.agent_tokens.lock().unwrap()
        .ensure(&child_name, octo_core::now_secs())?)
} else {
    None
};
```

The token is passed to the child via two env vars:

| env var              | purpose                                                    |
|----------------------|------------------------------------------------------------|
| `OCTO_AGENT_TOKEN`   | The token itself — sent as `X-Octo-Agent-Token` to lair.   |
| `LAIR_INTERNAL_URL`  | `http://127.0.0.1:8000` — where to POST callbacks.          |

### Why on disk and not just in memory

The supervisor has an [adoption path](../lair/src/agent_proc.rs:238) for lair restarts that happen with child processes still running. If the token map were in-memory only, those adopted children would carry an `OCTO_AGENT_TOKEN` env var that no longer matches anything in lair → they lose spawn capability silently. Persisting fixes that.

The threat from disk persistence is "someone with root inside the lair container reads `/data/lair/agent-tokens.json`" — but anyone with root in that container already controls lair, so it's not an escalation. Children run as a different uid (see below) and can't read 0600 root-owned files.

## The agent-token-gated endpoints

Two new routes on lair's HTTP server, both behind the `require_agent_token` middleware ([lair/src/lair.rs:154](../lair/src/lair.rs:154)):

```
POST   /agents/child           — spawn a new child owned by the caller
DELETE /agents/child/:name     — terminate one of the caller's descendants
```

The middleware:

1. Reads `X-Octo-Agent-Token` from the request.
2. Looks it up in the persisted store. Match → resolves to the caller's agent name. Miss → 403.
3. Attaches an `AgentCaller { name }` extension to the request, so handlers know who's calling without trusting the body.

The handlers are intentionally narrower than the operator endpoints:

- `agent_create_child` always sets `parent = Some(caller_name)`. The caller has no way to make a child whose parent is someone else, even by lying in the body.
- `agent_delete_child` checks the target name is in `registry.descendants_leaves_first(caller_name)` — i.e. transitively below the caller. Sibling, parent, unrelated agent, or lair itself → 403.

Caller identity comes from the token, not the body. There's no field the caller can populate to forge it.

## Spawn caps

Configured in `~/.octo/config.json` (defaults via `octo_core::resolve_agent_spawn_caps`):

| field                            | default | meaning                                                                 |
|----------------------------------|---------|-------------------------------------------------------------------------|
| `agent_spawn_max_depth`          | 3       | The new child's depth must be ≤ this. Top-level = 0, child = 1, etc.    |
| `agent_spawn_max_descendants`    | 5       | Caller's transitive descendant count after this spawn must be ≤ this.   |

Operator-spawned agents (depth 0) are unrestricted — the cap only fires inside the agent-token flow.

When a cap is hit, lair returns `403` with a human-readable message. The agent's system prompt instructs the model to accept the refusal rather than retry.

## Per-agent uid is the kernel-level enforcement

This is what stops sibling agents from impersonating each other. Without it, the token model would be paper-thin — agent A could `cat /proc/<B-pid>/environ` and steal B's `OCTO_AGENT_TOKEN`, then call `/agents/child` claiming to be B.

`lair/Dockerfile` pre-creates 100 users:

```Dockerfile
RUN useradd -u 10001 -M -N -s /bin/bash octo-agent && \
    for i in $(seq 0 99); do \
        useradd -u $((10100 + i)) -M -N -s /bin/bash octo-agent-$i; \
    done
```

`agent_proc::spawn` maps port 1:1 to uid:

```rust
const PORT_RANGE_BASE: u16 = 30100;
const UID_RANGE_BASE:  u32 = 10100;

fn uid_for_port(port: u16) -> (u32, u32) {
    if port >= PORT_RANGE_BASE && port < PORT_RANGE_BASE + 100 {
        let uid = UID_RANGE_BASE + (port - PORT_RANGE_BASE) as u32;
        (uid, uid)
    } else {
        (FALLBACK_AGENT_UID, FALLBACK_AGENT_GID)  // 10001
    }
}
```

So port 30100 → uid 10100, port 30199 → uid 10199. Each child runs as its own uid, and the Linux kernel's `/proc/<pid>/environ` access check refuses cross-uid reads. The 10001 fallback exists for the rare case where someone passes a port outside the standard range.

This is the bit that earns the rest of the design its security claims. Without it, the token store, the middleware, and the caps are all bypassable from a compromised sibling.

## Cascade terminate

`terminate_agent_by_name` is the single termination path used by every caller — operator CLI (`DELETE /agents/:name`), lair-LLM tool (`terminate_agent`), and agent tool (`DELETE /agents/child/:name`). The flow:

1. Snapshot the registry under the lock.
2. Compute `descendants_leaves_first(name)` — BFS by level, reversed so leaves come first.
3. For each descendant: `supervisor.terminate` (SIGTERM → 3s grace → SIGKILL → wait → `rm -rf` data dir), then drop the registry row, then drop the token from the persistent store.
4. Finally do the same for the target itself.

The poller fires once at the end so mobile sees the whole subtree disappear from the `agents` event in one frame.

Errors during descendant cleanup are logged and the loop continues — partial cleanup beats no cleanup. The only hard error case is "agent name not in registry," which returns up to the caller.

## The wire shape

The `agents` event pushed to mobile now includes `parent`:

```typescript
// mobile/src/wire.ts
interface AgentInfo {
  id:      string  // = name
  name:    string
  status:  string  // 'running' | 'stopped' | 'pending'
  kind:    string  // 'local' | 'remote'
  parent?: string  // omitted for operator-spawned roots
}
```

Operator-spawned agents have no `parent` field on the wire (it's `#[serde(skip_serializing_if = "Option::is_none")]`). Agent-spawned children carry their parent name. Mobile renders the sidebar as a tree by grouping children under their parent.

## Walkthrough: a real spawn

Agent `A` (depth 1, child of nothing on the registry... wait, this is the operator-spawned root, depth 0, no token). Let's make it `B` instead — `A` spawned `B`, and now `B` wants to spawn `C`.

```
1. B's LLM sees `spawn_agent` in its tool list (B has OCTO_AGENT_TOKEN).
2. B's LLM calls spawn_agent(name="C", git_url="https://github.com/foo/bar").
3. lair/src/agent.rs::exec_spawn_agent:
     POST http://127.0.0.1:8000/agents/child
     X-Octo-Agent-Token: <B's token>
     body: {"name":"C","git_url":"https://github.com/foo/bar"}

4. require_agent_token middleware:
     looks up token in AgentTokens → resolves to "B"
     attaches AgentCaller { name: "B" } extension

5. agent_create_child:
     reads config caps (3, 5)
     reg.depth_of("B") = 1 → new child depth = 2, OK
     reg.descendants_leaves_first("B").len() = 0 → new count = 1, OK
     calls exec_create_agent_for_parent(state, input, Some("B"))

6. exec_create_agent_for_parent:
     assigns port 30101 (30100 was A's)
     mints C's own token (parent is Some, so we mint)
     persists C's token to agent-tokens.json
     calls supervisor.spawn with:
       uid_for_port(30101) = 10101
       OCTO_AGENT_TOKEN=<C's token>
       LAIR_INTERNAL_URL=http://127.0.0.1:8000
       (no LAIR_MGMT_TOKEN — env_remove'd)
     adds registry row { name:"C", parent: Some("B"), port: 30101, ... }
     poll_trigger.notify_one() → mobile gets agents event with C nested under B

7. C boots:
     runs as uid 10101 inside the lair container
     reads OCTO_AGENT_TOKEN from its env → it can spawn grandchildren
     binds 127.0.0.1:30101
     starts its own agentic loop
```

If `A` later terminates `B`:

```
1. A's LLM calls terminate_agent(name="B").
2. exec_terminate_agent → DELETE http://127.0.0.1:8000/agents/child/B
     X-Octo-Agent-Token: <A's token>
3. require_agent_token resolves to "A".
4. agent_delete_child: is "B" in descendants_leaves_first("A")? Yes → OK.
5. terminate_agent_by_name("B"):
     descendants = ["C"]   (leaves first; C is the only descendant)
     terminate C:
       supervisor.stop("C") → SIGTERM pid 10101 → SIGKILL after 3s
       rm -rf /data/agents/C/
       remove registry row C
       remove agent_tokens["C"]
     terminate B:
       supervisor.stop("B") → SIGTERM → SIGKILL
       rm -rf /data/agents/B/
       remove registry row B
       remove agent_tokens["B"]
     poll_trigger.notify_one() → mobile sees B and C disappear simultaneously
```

## Layered security model summary

| Layer                                | What it stops                                                                                          |
|--------------------------------------|--------------------------------------------------------------------------------------------------------|
| `LAIR_MGMT_TOKEN` (X-Octo-Token)     | Agents can't hit operator endpoints — `POST /agents`, `DELETE /agents/:name`, `/start`, `/stop`.       |
| `OCTO_AGENT_TOKEN` (X-Octo-Agent-Token) | Agents can hit only `/agents/child` and `/agents/child/:name`. Caller identity comes from the token. |
| Per-agent uid (10100..10199)         | Sibling A can't read sibling B's `OCTO_AGENT_TOKEN` from `/proc/<B-pid>/environ`.                       |
| `parent` field + descendant check    | A valid token only lets you act on your own subtree — sibling, parent, and unrelated agents are 403.   |
| Depth + descendant caps              | Bounds tree size regardless of how many tokens are floating around.                                    |

Each layer fails independently. Removing any one of them weakens the model but doesn't collapse it; removing two of them probably does.

## What's intentionally not addressed

- **Children of children of children at depth 4+.** The default depth cap is 3. Operators who want deeper trees raise `agent_spawn_max_depth`. There's no architectural reason 3 is the right number — it's a starting default.
- **Concurrent fork races.** The depth/descendant caps are checked under the registry lock but the spawn itself isn't atomic with the count check. A burst of `spawn_agent` calls from the same parent could in theory overshoot by 1–2. Acceptable: the cap is a soft limit, not a security boundary.
- **Cross-VM ownership.** A `parent` value pointing at a remote agent works in the registry (the field is just a string), but cascade-terminate logs a warning for remote descendants and only drops the registry row — operators still have to terminate the VM via the cloud MCP. Local-only trees are the common case.

## Source pointers

- [lair/src/agent_tokens.rs](../lair/src/agent_tokens.rs) — the persistent token store; `AgentTokens::ensure / get / name_for_token / remove`.
- [lair/src/lair.rs:154](../lair/src/lair.rs) — `require_agent_token` middleware, `AgentCaller` extension.
- [lair/src/lair.rs](../lair/src/lair.rs) — `agent_create_child`, `agent_delete_child`, `exec_create_agent_for_parent`, `terminate_agent_by_name`.
- [lair/src/agent.rs](../lair/src/agent.rs) — `has_spawn_capability`, `spawn_agent_tool`, `terminate_agent_tool`, `exec_spawn_agent`, `exec_terminate_agent`.
- [lair/src/agent_proc.rs](../lair/src/agent_proc.rs) — `uid_for_port`, `SpawnParams.agent_token`, `SpawnParams.lair_internal_url`.
- [core/src/registry.rs](../core/src/registry.rs) — `AgentRecord.parent`, `depth_of`, `direct_children`, `descendants_leaves_first`.
- [core/src/lib.rs](../core/src/lib.rs) — `Config.agent_spawn_max_depth`, `agent_spawn_max_descendants`, `resolve_agent_spawn_caps`, `spawn_capability_note` (the prompt fragment children see when they have a token).
- [lair/Dockerfile](../lair/Dockerfile) — creates uids 10100..10199.
